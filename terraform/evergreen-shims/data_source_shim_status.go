package main

import (
	"context"
	"time"

	"github.com/hashicorp/terraform-plugin-sdk/v2/diag"
	"github.com/hashicorp/terraform-plugin-sdk/v2/helper/schema"
)

func dataSourceShimStatus() *schema.Resource {
	return &schema.Resource{
		ReadContext: dataSourceShimStatusRead,
		Timeouts: &schema.ResourceTimeout{
			Read: schema.DefaultTimeout(5 * time.Minute),
		},
		Schema: map[string]*schema.Schema{
			"name": {
				Type:        schema.TypeString,
				Required:    true,
				Description: "Name of the shim to query.",
			},
			"namespace": {
				Type:        schema.TypeString,
				Optional:    true,
				Default:     "default",
				Description: "Namespace of the shim.",
			},
			"status": {
				Type:        schema.TypeString,
				Computed:    true,
				Description: "Current status of the shim.",
			},
			"ready_replicas": {
				Type:        schema.TypeInt,
				Computed:    true,
				Description: "Number of ready replicas.",
			},
			"available_replicas": {
				Type:        schema.TypeInt,
				Computed:    true,
				Description: "Number of available replicas.",
			},
			"unavailable_replicas": {
				Type:        schema.TypeInt,
				Computed:    true,
				Description: "Number of unavailable replicas.",
			},
			"updated_replicas": {
				Type:        schema.TypeInt,
				Computed:    true,
				Description: "Number of updated replicas.",
			},
			"conditions": {
				Type:     schema.TypeList,
				Computed: true,
				Elem: &schema.Resource{
					Schema: map[string]*schema.Schema{
						"type": {
							Type:     schema.TypeString,
							Computed: true,
						},
						"status": {
							Type:     schema.TypeString,
							Computed: true,
						},
						"reason": {
							Type:     schema.TypeString,
							Computed: true,
						},
						"message": {
							Type:     schema.TypeString,
							Computed: true,
						},
						"last_transition_time": {
							Type:     schema.TypeString,
							Computed: true,
						},
					},
				},
			},
			"health": {
				Type:        schema.TypeString,
				Computed:    true,
				Description: "Health status from management API.",
			},
			"last_health_check": {
				Type:        schema.TypeString,
				Computed:    true,
				Description: "Timestamp of last health check.",
			},
		},
	}
}

func dataSourceShimStatusRead(ctx context.Context, d *schema.ResourceData, meta interface{}) diag.Diagnostics {
	provider := meta.(*ProviderConfig)
	_ = provider

	name := d.Get("name").(string)
	namespace := d.Get("namespace").(string)

	d.SetId(namespace + "/" + name)

	return nil
}
